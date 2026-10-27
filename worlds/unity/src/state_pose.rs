//! A controller's state as a motion ([`StateMotion`]: the clip and how its time runs) and as a pose ([`state_pose`]): the clip a state of one clip plays, at a time since the state was entered, on a rig the
//! reader built for that controller's Animator. This is one state of one layer alone: no transitions (nothing leaves the
//! state), no other layer, no layer weight or mask, no previous pose, no IK; the rig's rests are the base the clip's
//! channels replace.
//!
//! The time rule is Unity's documented one for a state of one clip: the clip is at `state time x speed + cycle offset x
//! the clip's length` seconds (the speed the state states, times the value of its speed parameter when it names one; the
//! cycle offset, a fraction of the clip, the state's or its parameter's), looped by the clip's own looping
//! ([`primase_core::Clip::extrapolation`]). It is not checked against an engine's pose here. A state that is a blend tree,
//! whose time is the weighted length of its clips, is refused by name, as is one whose time or mirror is set by a
//! parameter or flag this does not apply.

use primase_animation::pose::{Evaluated, evaluate};
use primase_core::{Clip, ClipBinding, Entity, RigInstance, Skeleton, World};

use crate::animator::UnityAnimatorController;
use crate::blend_tree::Parameters;
use crate::components::UnityObject;
use crate::invalid;
use unity_formats::player::ObjRef;
use unity_formats::{Error, Result};

/// What a state of one clip plays: the clip, and how the state's time maps to the clip's. Made from the controller alone (no
/// world), so a program can list a controller's states and which of them it can play.
#[derive(Clone, Debug, PartialEq)]
pub struct StateMotion {
    /// The clip, as the controller's clip list names it.
    pub clip: ObjRef,
    /// Clip seconds per state second: the state's speed times its speed parameter's value, when it names one.
    pub speed: f64,
    /// The fraction of the clip's length the state starts in (the state's, or its parameter's value).
    pub cycle_offset: f32,
}

impl StateMotion {
    /// Layer `layer`'s state `state` of `controller` at these parameter values; `Unsupported` (naming the state and why) for
    /// one this does not play, an error for an index that is not there.
    pub fn of(
        controller: &UnityAnimatorController,
        layer: usize,
        state: usize,
        parameters: &Parameters,
    ) -> Result<StateMotion> {
        let l = controller
            .layers
            .get(layer)
            .ok_or_else(|| invalid(format!("layer {layer} of {}", controller.layers.len())))?;
        let machine = usize::try_from(l.state_machine)
            .ok()
            .and_then(|m| controller.state_machines.get(m))
            .ok_or_else(|| invalid(format!("layer {layer} runs state machine {}", l.state_machine)))?;
        let s = machine.states.get(state).ok_or_else(|| {
            invalid(format!(
                "state {state} of {} in layer {layer}",
                machine.states.len()
            ))
        })?;
        let name = || {
            controller
                .name_of(s.name_id)
                .map_or_else(|| format!("state {state}"), |n| format!("state {n:?}"))
        };
        if s.time_param != 0 {
            return Err(Error::Unsupported(format!(
                "{}: its time is set by a parameter",
                name()
            )));
        }
        if s.mirror || s.mirror_param != 0 {
            return Err(Error::Unsupported(format!("{}: mirrored", name())));
        }
        let tree = match s.blend_tree_index.as_slice() {
            [i] => usize::try_from(*i)
                .ok()
                .and_then(|i| s.blend_trees.get(i))
                .ok_or_else(|| invalid(format!("{}: blend tree {i} of {}", name(), s.blend_trees.len())))?,
            sets => {
                return Err(Error::Unsupported(format!(
                    "{}: {} motion sets (a synchronised layer's overrides)",
                    name(),
                    sets.len()
                )));
            }
        };
        let leaf = match tree.first() {
            Some(n) if n.children.is_empty() => n,
            Some(_) => {
                return Err(Error::Unsupported(format!(
                    "{}: a blend tree (the time of a weighted blend is not stated here)",
                    name()
                )));
            }
            None => return Err(Error::Unsupported(format!("{}: no motion", name()))),
        };
        let clip = controller
            .clips
            .get(leaf.clip as usize)
            .ok_or_else(|| Error::Unsupported(format!("{}: no clip", name())))?
            .clone()
            .ok_or_else(|| Error::Unsupported(format!("{}: its clip is an empty slot", name())))?;
        let speed = f64::from(s.speed)
            * if s.speed_param == 0 {
                1.0
            } else {
                f64::from(parameters.get(s.speed_param)?)
            };
        let cycle_offset = if s.cycle_offset_param == 0 {
            s.cycle_offset
        } else {
            parameters.get(s.cycle_offset_param)?
        };
        Ok(StateMotion {
            clip,
            speed,
            cycle_offset,
        })
    }

    /// The time in the clip, in seconds (before the clip's own looping), at `state_time` seconds after the state was entered,
    /// for a clip of this length.
    pub fn clip_time(&self, state_time: f64, clip_duration: f32) -> f64 {
        state_time * self.speed + f64::from(self.cycle_offset) * f64::from(clip_duration)
    }
}

/// Layer `layer`'s state `state` of `controller` at `time` seconds after it was entered, on `rig` (an entity holding a
/// `core.rig_instance`): the clip's pose over the skeleton's rests, with its morphs, controls and root motion (see
/// [`primase_animation::pose::Evaluated`]). The clip must be one the reader bound to this rig (a clip that drives no joint
/// of the rig is not made, so a state playing it is an error).
pub fn state_pose(
    world: &World,
    controller: &UnityAnimatorController,
    layer: usize,
    state: usize,
    rig: Entity,
    parameters: &Parameters,
    time: f64,
) -> Result<Evaluated> {
    let motion = StateMotion::of(controller, layer, state, parameters)?;
    let skeleton_entity = world
        .get::<RigInstance>(rig)
        .ok_or_else(|| invalid(format!("{rig:?} is not a rig")))?
        .skeleton;
    let skeleton: &Skeleton = world
        .get::<Skeleton>(skeleton_entity)
        .ok_or_else(|| invalid("the rig's skeleton is gone"))?;
    let clip_entity = world
        .iter::<ClipBinding>()
        .filter(|(_, b)| b.rig == Some(rig))
        .map(|(_, b)| b.clip)
        .find(|c| world.get::<UnityObject>(*c).map(|o| &o.0) == Some(&motion.clip))
        .ok_or_else(|| {
            invalid(format!(
                "state {state} of layer {layer}: the rig has no clip {}/{} bound (a clip that drives none of its joints is not made)",
                motion.clip.file, motion.clip.path_id
            ))
        })?;
    let clip: &Clip = world
        .get::<Clip>(clip_entity)
        .ok_or_else(|| invalid("a bound clip is gone"))?;
    evaluate(
        clip,
        skeleton_entity,
        skeleton,
        None,
        motion.clip_time(time, clip.duration),
    )
    .map_err(|e| invalid(format!("state {state} of layer {layer}: {}", e.0)))
}

#[cfg(test)]
mod tests {
    use primase_core::{Channel, ClipTarget, Extrapolation, Interpolation, Joint, JointId, Property, Rest};

    use super::*;
    use crate::components::Stored;
    use unity_formats::animator::{BlendNode, Layer, State, StateMachine};
    use unity_formats::player::ObjRef;

    fn leaf(clip: u32) -> BlendNode {
        BlendNode {
            blend_type: 0,
            event_id: u32::MAX,
            event_y_id: u32::MAX,
            children: vec![],
            thresholds: vec![],
            positions: vec![],
            magnitudes: vec![],
            pair_vectors: vec![],
            pair_avg_mag_inv: vec![],
            neighbors: vec![],
            direct_events: vec![],
            normalized_blend_values: false,
            clip,
            duration: 1.0,
            cycle_offset: 0.0,
            mirror: false,
        }
    }

    fn state(speed: f32, cycle_offset: f32) -> State {
        State {
            transitions: vec![],
            blend_tree_index: vec![0],
            blend_trees: vec![vec![leaf(1)]],
            name_id: 7,
            path_id: 7,
            full_path_id: 7,
            tag_id: 0,
            speed_param: 0,
            mirror_param: 0,
            cycle_offset_param: 0,
            time_param: 0,
            speed,
            cycle_offset,
            ik_on_feet: false,
            write_default_values: true,
            looping: true,
            mirror: false,
        }
    }

    fn controller(states: Vec<State>) -> UnityAnimatorController {
        Stored(unity_formats::animator::UnityAnimatorController {
            controller: ObjRef {
                file: "f".into(),
                path_id: 1,
            },
            name: "c".into(),
            layers: vec![Layer {
                state_machine: 0,
                synchronized_layer: 0,
                body_mask: [u32::MAX; 3],
                skeleton_mask: vec![],
                binding: 5,
                blending_mode: 0,
                default_weight: 1.0,
                ik_pass: false,
                synced_layer_affects_timing: false,
            }],
            state_machines: vec![StateMachine {
                states,
                any_state_transitions: vec![],
                selectors: vec![],
                default_state: 0,
                synchronized_layer_count: 1,
            }],
            parameters: vec![],
            default_floats: vec![],
            default_ints: vec![],
            default_bools: vec![],
            // clip 1 is the one bound to the rig; slot 0 is a null pointer
            clips: vec![
                None,
                Some(ObjRef {
                    file: "f".into(),
                    path_id: 9,
                }),
            ],
            behaviours: vec![],
            tos: vec![(7, "Turning".into())],
        })
    }

    /// A rig of one joint (id 1, at rest turned not at all) and a clip, bound to it, that turns the joint a half turn about
    /// z over 2 s linearly, looping.
    fn world() -> (World, Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: vec![Joint {
                    id: JointId(1),
                    name: "j".into(),
                    parent: None,
                    rest: Rest::Trs {
                        translation: [0.0; 3],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0; 3],
                    },
                }],
            },
        );
        let rig = w.spawn();
        w.insert(rig, RigInstance { skeleton });
        let clip = w.spawn();
        w.insert(
            clip,
            Clip {
                name: "turn".into(),
                duration: 2.0,
                extrapolation: Extrapolation::Loop,
                channels: vec![Channel {
                    target: ClipTarget::Joint {
                        skeleton,
                        joint: JointId(1),
                    },
                    property: Property::Rotation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 2.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                }],
                root_motion: None,
                additive: None,
            },
        );
        w.insert(
            clip,
            Stored(ObjRef {
                file: "f".into(),
                path_id: 9,
            }),
        );
        let binding = w.spawn();
        w.insert(
            binding,
            ClipBinding {
                clip,
                rig: Some(rig),
                meshes: vec![],
            },
        );
        (w, rig)
    }

    fn angle(e: &Evaluated) -> f64 {
        let Rest::Trs { rotation, .. } = e.pose.joints[0].1 else {
            panic!("a driven joint is TRS")
        };
        // a rotation about z: 2 atan2(z, w)
        2.0 * f64::from(rotation[2]).atan2(f64::from(rotation[3]))
    }

    /// Hand-worked: the clip turns a half turn over 2 s. At speed 2 the state's 0.25 s are the clip's 0.5 s, and a cycle
    /// offset of 0.25 adds a quarter of the clip (0.5 s), so the clip is at 1 s: a quarter turn (the angle of the
    /// rotation about z is pi / 2). With neither it is at 0.5 s, an eighth of a turn; the clip loops, so 4.25 s of state
    /// at speed 2 is at 8.5 s, which is 0.5 s.
    #[test]
    fn a_one_clip_state_plays_its_clip_at_speed_and_cycle_offset() {
        let (w, rig) = world();
        let p = Parameters::defaults(&controller(vec![])).unwrap();
        let at = |speed: f32, cycle: f32, t: f64| {
            let c = controller(vec![state(speed, cycle)]);
            angle(&state_pose(&w, &c, 0, 0, rig, &p, t).unwrap())
        };
        let half = std::f64::consts::FRAC_PI_2;
        assert!((at(2.0, 0.25, 0.25) - half).abs() < 1e-6);
        assert!((at(2.0, 0.0, 0.25) - half / 2.0).abs() < 1e-6);
        assert!((at(2.0, 0.0, 4.25) - half / 2.0).abs() < 1e-6);
    }

    /// The motion of a state is its clip (by object), its speed and its cycle offset, and says where in the clip a state time
    /// is: 0.25 s at speed 2 is 0.5 s, plus a quarter of a 2 s clip.
    #[test]
    fn a_state_motion_maps_state_time_to_clip_time() {
        let p = Parameters::defaults(&controller(vec![])).unwrap();
        let m = StateMotion::of(&controller(vec![state(2.0, 0.25)]), 0, 0, &p).unwrap();
        assert_eq!(
            m,
            StateMotion {
                clip: ObjRef {
                    file: "f".into(),
                    path_id: 9
                },
                speed: 2.0,
                cycle_offset: 0.25
            }
        );
        assert_eq!(m.clip_time(0.25, 2.0), 1.0);
    }

    /// What the state cannot be played as is refused by name, never posed as if it could: a blend tree, a time or mirror
    /// parameter, an empty clip slot, a clip the rig has no binding for, a state or layer that is not there.
    #[test]
    fn what_cannot_be_played_is_refused_by_name() {
        let (w, rig) = world();
        let p = Parameters::defaults(&controller(vec![])).unwrap();
        let err = |s: State| {
            let c = controller(vec![s]);
            state_pose(&w, &c, 0, 0, rig, &p, 0.0).unwrap_err().to_string()
        };
        let mut tree = state(1.0, 0.0);
        tree.blend_trees[0][0].children = vec![0];
        assert!(err(tree).contains("a blend tree"), "named by its name");
        let mut timed = state(1.0, 0.0);
        timed.time_param = 3;
        assert!(err(timed).contains("\"Turning\": its time is set by a parameter"));
        let mut mirrored = state(1.0, 0.0);
        mirrored.mirror = true;
        assert!(err(mirrored).contains("mirrored"));
        let mut empty = state(1.0, 0.0);
        empty.blend_trees[0][0].clip = 0;
        assert!(err(empty).contains("an empty slot"));
        let mut missing = state(1.0, 0.0);
        missing.blend_trees[0][0].clip = 5;
        assert!(err(missing).contains("no clip"));
        let mut sets = state(1.0, 0.0);
        sets.blend_tree_index = vec![0, 0];
        assert!(err(sets).contains("2 motion sets"));
        let c = controller(vec![state(1.0, 0.0)]);
        assert!(state_pose(&w, &c, 0, 3, rig, &p, 0.0).is_err());
        assert!(state_pose(&w, &c, 2, 0, rig, &p, 0.0).is_err());
        // a rig the clip is not bound to
        let mut other = World::new();
        let skeleton = other.spawn();
        other.insert(skeleton, Skeleton { joints: vec![] });
        let rig2 = other.spawn();
        other.insert(rig2, RigInstance { skeleton });
        let e = state_pose(&other, &c, 0, 0, rig2, &p, 0.0)
            .unwrap_err()
            .to_string();
        assert!(e.contains("has no clip f/9 bound"), "{e}");
    }
}
